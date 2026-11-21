use super::*;
use crate::compose::input::{self, Action, OnPage};
use crate::screens::canvas::Shape;
use crate::screens::chrome::{ACCENT, CARD, PRESSED, RAISED, Rect, SECONDARY, WHITE};
use crate::screens::scroll::Layout;
use crate::screens::workout::{Page, View};
use firmware_shell::rendering::font::FaceRole;

fn view_on(
    layout: &'static crate::screens::Layout,
    cursor: usize,
    editing: bool,
    values: &[(&str, &str)],
    status: &str,
    scroll: i32,
) -> View<'static> {
    let mut view = View::new(Page::Settings, cursor, None);
    view.layout = layout;
    view.editing = editing;
    view.scroll = scroll;
    view.compose_main();
    let lines = view.show_settings();
    for (slot, (value, note)) in values.iter().enumerate() {
        let _ = lines[VALUES + slot].push_str(value);
        let _ = lines[NOTES + slot].push_str(note);
    }
    let _ = lines[STATUS].push_str(status);
    view.prepare();
    view
}

fn view(cursor: usize, editing: bool, values: &[(&str, &str)], status: &str) -> View<'static> {
    view_on(&crate::screens::BASE, cursor, editing, values, status, 0)
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

/// The rows' page on `layout`'s panel.
fn page(layout: &'static crate::screens::Layout) -> Layout {
    *view_on(layout, 0, false, &[], "SAVED", 0)
        .layout()
        .expect("Settings scrolls its rows")
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
    let list = page(&crate::screens::BASE);
    let mut moving = profile(1, false);
    let colors: std::vec::Vec<_> = panels(&moving).iter().map(|(_, c)| *c).collect();
    assert_eq!(colors, [CARD, ACCENT, RAISED, CARD, CARD], "no pads");
    let left = list.column.left;
    let second = list.top + list.column.rect(1).top;
    assert_eq!(moving.pixel(left + 6, second + 20), RAISED);
    assert_eq!(moving.pixel(100, second - 2), ACCENT, "ring");
    assert!(moving.canvas().shows("PROFILE") && moving.canvas().shows("Z4 195-226 W"));
    assert!(moving.canvas().shows("FTP WATTS") && moving.canvas().shows("SAVED"));
    assert_eq!(
        moving.canvas().find("Z4 195-226 W").map(|run| run.color),
        Some(SECONDARY)
    );

    let mut editing = profile(0, true);
    let colors: std::vec::Vec<_> = panels(&editing).iter().map(|(_, c)| *c).collect();
    assert_eq!(colors, [ACCENT, RAISED, RAISED]);
    let value = editing.canvas().find("215").unwrap();
    assert_eq!(value.color, WHITE);
    assert_eq!(
        value.line.face().cap,
        crate::screens::BASE.scaled.face(FaceRole::Value).cap
    );
    assert!(editing.canvas().shows("FTP WATTS") && !editing.canvas().shows("LTHR BPM"));
    assert!(editing.canvas().shows("-5") && editing.canvas().shows("+5"));
    assert!(editing.canvas().shows("PROFILE") && editing.canvas().shows("SAVED"));
    let (pad, _) = panels(&editing)[1];
    assert_eq!(editing.pixel(pad.left + 6, pad.top + 20), RAISED);
}

#[test]
fn body_numbers_move_a_unit_at_a_time_and_say_when_unset() {
    let weight = Sections::All.field(Row::Weight).unwrap();
    let values = [("NOT SET", "ADD FOR W/KG")];
    let editing = view(weight, true, &values, "SAVED");
    assert!(editing.canvas().shows("WEIGHT KG") && editing.canvas().shows("NOT SET"));
    assert!(editing.canvas().shows("-1") && editing.canvas().shows("+1"));
    assert!(!editing.canvas().shows("+5"));
    assert_eq!(Row::Weight.step(), 1);
    assert_eq!(Row::Age.step(), 1);
    assert_eq!(Row::Ftp.step(), STEP);
    assert_eq!(Row::Sex.kind(), Kind::Choice);
}

#[test]
fn extreme_settings_never_overlap_or_clip() {
    let long = [
        ("1000", "Z4 905-1050 W"),
        ("220", "Z5 FROM 220 BPM"),
        ("SEND RIDES", ""),
        ("99H LEFT", "120 RIDES"),
        ("OFF", "NO CLIMB POP-UP"),
    ];
    for layout in crate::screens::panels::ALL {
        let list = page(layout);
        let screen = Rect {
            bottom: list.stretched_bottom(),
            ..layout.scaled.screen()
        };
        for cursor in 0..Sections::All.fields() {
            let count = Sections::All.rows(cursor).0.len();
            let number = Sections::All.row(cursor).kind() == Kind::Number;
            for editing in [false, true] {
                let editor = editing && number;
                // Every scroll of the rows; a section that fits has only 0.
                let scrolls = if editor {
                    0..=0
                } else {
                    0..=list.reveal(count - 1, 0).max(0)
                };
                let statuses = ["SAVED", "SAVE FAILED, RETRYING", "NOT SAVED: DATA KEPT"];
                for status in statuses {
                    for scroll in scrolls.clone() {
                        let mut view = view_on(layout, cursor, editing, &long, status, scroll);
                        view.prepare().check_layout();
                        let whole = (0..count).filter(|&index| list.whole(index, scroll));
                        let shown: std::vec::Vec<usize> = if editor {
                            std::vec![0]
                        } else {
                            whole.collect()
                        };
                        let mut texts = std::vec![status];
                        for (value, note) in shown.iter().map(|&index| long[index]) {
                            texts.push(value);
                            if !note.is_empty() {
                                texts.push(note);
                            }
                        }
                        let what = std::format!(
                            "{cursor} {editing} {status} at {scroll} on {:?}",
                            layout.scaled.panel
                        );
                        view.canvas().assert_laid_out(screen, &texts, &what);
                    }
                }
            }
        }
    }
}

#[test]
fn a_tap_on_a_pad_steps_the_number_only_while_editing() {
    let editing = profile(0, true);
    let laid = editing.composed().expect("Settings has a tree");
    let solved = laid.solved();
    let (pads, buttons) = (panels(&editing), laid.buttons());
    let tap = |rect: Rect| {
        let (x, y) = (
            rect.center_x() as u16,
            (rect.top + rect.height() / 2) as u16,
        );
        input::hit(buttons, &solved, x, y).map(|(_, action)| action)
    };
    assert_eq!(tap(pads[1].0), Some(Action::Page(OnPage::Lower)));
    assert_eq!(tap(pads[2].0), Some(Action::Page(OnPage::Raise)));
    assert_eq!(tap(pads[0].0), None, "the field is no pad");
}

#[test]
fn a_long_section_scrolls_and_shows_where_it_is() {
    let list = page(&crate::screens::BASE);
    let display = Sections::All.field(Row::Brightness).unwrap();
    let rows = Sections::All.rows(display).0.len();
    assert_eq!(rows, 5);
    let x = list.column.right + crate::screens::BASE.scaled.unit(4) + 1;
    let bar = |view: &mut View, y| view.pixel(x, y);
    let scrolled = |offset| view_on(&crate::screens::BASE, display, false, &[], "SAVED", offset);
    // From the top: the thumb at the top of the track.
    let mut top = scrolled(0);
    assert!(top.canvas().shows("DISPLAY"));
    assert_eq!(bar(&mut top, list.top + 2), SECONDARY);
    assert_eq!(bar(&mut top, list.bottom - 2), CARD);
    // Scrolled to the end: the thumb at the bottom, the fifth row whole.
    let end = list.content(rows) as i32 - list.viewport() as i32;
    assert!(end > 0);
    let mut bottom = scrolled(end);
    assert_eq!(bar(&mut bottom, list.top + 2), CARD);
    assert_eq!(bar(&mut bottom, list.bottom - 2), SECONDARY);
    assert!(list.whole(4, end));
    // A short section has no bar.
    assert_eq!(bar(&mut view(0, false, &[], "SAVED"), list.top + 2), 0);
}

#[test]
fn a_row_under_a_resting_finger_is_pressed() {
    let mut view = View::new(Page::Settings, 0, None);
    view.compose_main();
    let _ = view.show_settings()[STATUS].push_str("SAVED");
    view.press_setting(Some(1));
    view.prepare();
    let colors: std::vec::Vec<_> = panels(&view).iter().map(|(_, c)| *c).collect();
    assert_eq!(colors, [ACCENT, RAISED, PRESSED, CARD, CARD]);
}
