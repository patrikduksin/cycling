//! Settings: grouped sections of rows, one section per page.
//!
//! Buttons keep their system roles. BottomLeft moves to the next row (the
//! page follows into the next section), BottomRight acts on it and TopLeft
//! goes back Home. Acting cycles a choice, opens a page or edits a number.
//! While a number is edited, BottomLeft lowers and BottomRight raises it by
//! `STEP`, and TopLeft finishes. Touch does the same directly: swipe between
//! sections, tap a row to act on it, and tap the large minus and plus pads
//! while editing. The runtime prepares each row's value and note; this draws
//! them and maps touches to `Target`s.
use super::canvas::{Canvas, Shape};
use super::chrome::{
    ACCENT, AMBER, Align, BODY, CAPTION, CARD, LARGE, Line, RAISED, Rect, SECONDARY, TITLE, VALUE,
    WHITE,
};
use super::workout::View;

/// Every row, in BottomLeft order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Ftp,
    Threshold,
    Bests,
    Brightness,
    AutoDim,
    Daylight,
    Theme,
    TimeZone,
    AutoPause,
    AutoLap,
    Wheel,
    WheelMode,
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
}

/// Sections in order, each with its title and the rows it holds.
pub const SECTIONS: [(&str, &[Row]); 8] = [
    ("PROFILE", &[Row::Ftp, Row::Threshold, Row::Bests]),
    (
        "DISPLAY",
        &[Row::Brightness, Row::AutoDim, Row::Daylight, Row::Theme],
    ),
    ("CLOCK", &[Row::TimeZone]),
    (
        "RIDE",
        &[Row::AutoPause, Row::AutoLap, Row::Wheel, Row::WheelMode],
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

/// The most rows a section shows.
pub const SECTION_ROWS: usize = 4;
/// All rows, the cursor's range.
pub const ROWS: usize = {
    let mut count = 0;
    let mut i = 0;
    while i < SECTIONS.len() {
        count += SECTIONS[i].1.len();
        i += 1;
    }
    count
};

impl Row {
    /// The row at cursor position `index`, wrapping.
    pub fn at(index: usize) -> Self {
        let mut index = index % ROWS;
        for (_, rows) in SECTIONS {
            if index < rows.len() {
                return rows[index];
            }
            index -= rows.len();
        }
        unreachable!("index is below ROWS")
    }

    /// The row's cursor position.
    pub fn index(self) -> usize {
        let mut first = 0;
        for (_, rows) in SECTIONS {
            if let Some(i) = rows.iter().position(|row| *row == self) {
                return first + i;
            }
            first += rows.len();
        }
        unreachable!("every row is in a section")
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ftp => "FTP WATTS",
            Self::Threshold => "LTHR BPM",
            Self::Bests => "BESTS",
            Self::Brightness => "BRIGHTNESS",
            Self::AutoDim => "DIM AFTER",
            Self::Daylight => "DAY / NIGHT",
            Self::Theme => "THEME",
            Self::TimeZone => "TIME ZONE",
            Self::AutoPause => "AUTO PAUSE",
            Self::AutoLap => "AUTO LAP",
            Self::Wheel => "WHEEL",
            Self::WheelMode => "WHEEL MODE",
            Self::Sounds => "SOUNDS",
            Self::RadarAlerts => "RADAR COUNT",
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

    pub const fn action(self) -> Option<SettingsAction> {
        match self {
            Self::Sensors => Some(SettingsAction::Sensors),
            Self::Phone => Some(SettingsAction::ConnectPhone),
            Self::Bests => Some(SettingsAction::Bests),
            _ => None,
        }
    }

    pub const fn kind(self) -> Kind {
        match self {
            Self::Ftp | Self::Threshold | Self::Wheel => Kind::Number,
            Self::Brightness
            | Self::AutoDim
            | Self::Daylight
            | Self::Theme
            | Self::TimeZone
            | Self::AutoPause
            | Self::AutoLap
            | Self::WheelMode
            | Self::Sounds
            | Self::RadarAlerts
            | Self::FuelAlert
            | Self::CadenceLow
            | Self::SunsetAlert
            | Self::HeartHigh
            | Self::HeartLow
            | Self::PowerHigh
            | Self::PowerLow => Kind::Choice,
            Self::Sensors | Self::Phone | Self::Bests => Kind::Open,
            Self::Storage | Self::Firmware => Kind::Info,
        }
    }
}

/// The section holding cursor position `index`, and its first position.
pub fn section(index: usize) -> (usize, usize) {
    let mut first = 0;
    for (section, (_, rows)) in SECTIONS.iter().enumerate() {
        if index % ROWS < first + rows.len() {
            return (section, first);
        }
        first += rows.len();
    }
    unreachable!("index is below ROWS")
}

/// The first cursor position of `section`, wrapping.
pub fn first_of(section: usize) -> usize {
    SECTIONS[..section % SECTIONS.len()]
        .iter()
        .map(|(_, rows)| rows.len())
        .sum()
}

/// Each press or tap moves a number by this much: watts, bpm or a wheel's mm.
pub const STEP: u16 = 5;

/// Lines the runtime prepares for this page.
pub const STATUS: usize = 0;
/// Each shown row's value, in section order; the edited number's value.
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
const FOCUS: usize = 3;
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

fn row_rect(slot: usize) -> Rect {
    let top = ROW_TOP + slot * ROW_PITCH;
    Rect {
        left: LEFT,
        top,
        right: RIGHT,
        bottom: top + ROW_HEIGHT,
    }
}

/// The target under a touch at panel point (`x`, `y`) on the section that
/// holds `cursor`: its rows, or the pads while a number is edited.
pub fn target(x: usize, y: usize, cursor: usize, editing: bool) -> Option<Target> {
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
    let (section, first) = section(cursor);
    (0..SECTIONS[section].1.len())
        .find(|slot| {
            let rect = row_rect(*slot);
            (rect.top..rect.bottom).contains(&y)
        })
        .map(|slot| Target::Row(first + slot))
}

/// The section's rows, or the number editor, with the save status.
pub(super) fn prepare(view: &View) -> Canvas {
    let mut canvas = Canvas::default();
    let (section, first) = section(view.cursor);
    let (title, rows) = SECTIONS[section];
    let row = Row::at(view.cursor);
    if view.editing && row.kind() == Kind::Number {
        editor(&mut canvas, view, title, row);
    } else {
        canvas.fit(
            &[&TITLE],
            title.as_bytes(),
            RIGHT - LEFT,
            LEFT,
            TITLE_BASE,
            Align::Left,
            WHITE,
        );
        for (slot, row) in rows.iter().enumerate() {
            row_card(&mut canvas, view, slot, *row, first + slot == view.cursor);
        }
    }
    // A toast takes the bottom band; the save status waits under it.
    if view.toast.is_some() {
        return canvas;
    }
    let status = view.lines[STATUS].as_bytes();
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
fn row_card(canvas: &mut Canvas, view: &View, slot: usize, row: Row, focused: bool) {
    let rect = row_rect(slot);
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
        color: if focused { RAISED } else { CARD },
    });
    let (left, right) = (LEFT + INSET, RIGHT - INSET);
    canvas.caption(
        row.label().as_bytes(),
        right - left,
        left,
        rect.top + 19,
        Align::Left,
    );
    let base = rect.top + 38;
    let value = canvas.fit(
        &[&BODY, &CAPTION],
        view.lines[VALUES + slot].as_bytes(),
        right - left,
        left,
        base,
        Align::Left,
        WHITE,
    );
    // A note that does not fit whole beside the value is left out.
    let note = view.lines[NOTES + slot].as_bytes();
    let room = right.saturating_sub(value.right() + 12);
    if !note.is_empty() && Line::<24>::fit(&[&CAPTION], note, room).text().len() == note.len() {
        canvas.caption(note, room, right, base, Align::Right);
    }
}

/// The edited number on an accent field, with its note, then the pads.
fn editor(canvas: &mut Canvas, view: &View, title: &str, row: Row) {
    canvas.caption(title.as_bytes(), RIGHT - LEFT, LEFT, 44, Align::Left);
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
        view.lines[VALUES].as_bytes(),
        width,
        left,
        rect.top + 60,
        Align::Left,
        WHITE,
    );
    canvas.fit(
        &[&CAPTION],
        view.lines[NOTES].as_bytes(),
        width,
        left,
        rect.top + 80,
        Align::Left,
        WHITE,
    );
    for (left, right, label) in [
        (LEFT, PAD_SPLIT - PAD_GAP, b"-5"),
        (PAD_SPLIT + PAD_GAP, RIGHT, b"+5"),
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
        for (slot, (value, note)) in values.iter().enumerate() {
            let _ = view.lines[VALUES + slot].push_str(value);
            let _ = view.lines[NOTES + slot].push_str(note);
        }
        let _ = view.lines[STATUS].push_str(status);
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
        view.canvas
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
        assert_eq!(ROWS, 25);
        for index in 0..ROWS {
            assert_eq!(Row::at(index).index(), index);
            let (section, first) = section(index);
            assert_eq!(first_of(section), first);
            assert!(SECTIONS[section].1.len() <= SECTION_ROWS);
        }
        assert_eq!(Row::at(ROWS), Row::Ftp, "wraps");
        assert_eq!(section(Row::TimeZone.index()), (2, Row::TimeZone.index()));
        assert_eq!(section(Row::Sounds.index()), (4, Row::Sounds.index()));
        assert_eq!(section(Row::PowerLow.index()), (6, Row::HeartHigh.index()));
        assert_eq!(first_of(SECTIONS.len()), 0);
    }

    #[test]
    fn rings_the_focused_row_and_fills_the_edited_number() {
        let moving = profile(1, false);
        let colors: std::vec::Vec<_> = panels(&moving).iter().map(|(_, c)| *c).collect();
        assert_eq!(colors, [CARD, ACCENT, RAISED, CARD], "no pads");
        assert_eq!(moving.pixel(LEFT, ROW_TOP), 0, "rounded corner");
        let second = row_rect(1);
        assert_eq!(moving.pixel(LEFT + 1, second.top + 20), ACCENT, "ring");
        assert_eq!(moving.pixel(LEFT + 6, second.top + 20), RAISED);
        assert!(moving.canvas.shows("PROFILE") && moving.canvas.shows("Z4 195-226 W"));
        assert_eq!(moving.canvas.find("215").map(|run| run.color), Some(WHITE));
        // No permanent button labels.
        assert!((0..240).all(|x| (306..320).all(|y| moving.pixel(x, y) == 0)));

        let editing = profile(0, true);
        let colors: std::vec::Vec<_> = panels(&editing).iter().map(|(_, c)| *c).collect();
        assert_eq!(colors, [ACCENT, RAISED, RAISED]);
        assert!(editing.canvas.find("215").unwrap().line.face().cap == VALUE.cap);
        assert!(editing.canvas.shows("FTP WATTS") && !editing.canvas.shows("LTHR BPM"));
        assert!(editing.canvas.shows("-5") && editing.canvas.shows("+5"));
        assert_eq!(editing.pixel(LEFT + 2, PAD_TOP + 20), RAISED);
    }

    /// Long values, notes and statuses still lay out whole on every section.
    #[test]
    fn extreme_settings_never_overlap_or_clip() {
        let screen = Rect {
            left: 0,
            top: 24,
            right: 240,
            bottom: 320,
        };
        let long = [
            ("1000", "Z4 905-1050 W"),
            ("220", "Z5 FROM 220 BPM"),
            ("SEND RIDES", ""),
            ("99H LEFT", "120 RIDES"),
        ];
        for cursor in 0..ROWS {
            for editing in [false, true] {
                for status in ["SAVED", "SAVE FAILED, RETRYING", "NOT SAVED: DATA KEPT"] {
                    let view = view(cursor, editing, &long, status);
                    let (section, _) = section(cursor);
                    let shown = if editing && Row::at(cursor).kind() == Kind::Number {
                        1
                    } else {
                        SECTIONS[section].1.len()
                    };
                    let mut texts = std::vec![status];
                    for (value, note) in &long[..shown] {
                        texts.push(value);
                        if !note.is_empty() {
                            texts.push(note);
                        }
                    }
                    view.canvas.assert_laid_out(
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
        assert_eq!(target(100, row(0), 0, false), Some(Target::Row(0)));
        assert_eq!(target(200, row(1), 0, false), Some(Target::Row(1)));
        assert_eq!(target(100, row(2), 0, false), Some(Target::Row(2)));
        assert_eq!(
            target(100, row(3), 0, false),
            None,
            "profile has three rows"
        );
        let device = Row::Sensors.index();
        assert_eq!(
            target(100, row(3), device, false),
            Some(Target::Row(device + 3))
        );
        assert_eq!(target(100, ROW_TOP + ROW_HEIGHT + 2, 0, false), None);
        let ride = Row::TimeZone.index();
        assert_eq!(target(30, PAD_TOP + 10, ride, false), None);
        assert_eq!(target(30, PAD_TOP + 10, 0, true), Some(Target::Lower));
        assert_eq!(target(200, PAD_TOP + 10, 0, true), Some(Target::Raise));
        assert_eq!(target(100, row(0), 0, true), None);
        assert_eq!(target(5, PAD_TOP + 10, 0, true), None);
    }
}
