//! Settings: the training profile, one field at a time.
//!
//! Buttons keep their system roles. BottomLeft moves to the next field,
//! BottomRight edits it and TopLeft goes back Home. While a field is being
//! edited, BottomLeft lowers and BottomRight raises it by `STEP`, and TopLeft
//! finishes. Touch does the same directly: tap a field to edit it, then tap
//! the large minus and plus pads. The runtime prepares the lines; this draws
//! them and maps touches to `Target`s.
use super::canvas::{Canvas, Shape};
use super::chrome::{
    ACCENT, AMBER, Align, BODY, CAPTION, CARD, LARGE, RAISED, Rect, SECONDARY, VALUE, WHITE,
};
use super::workout::View;

const RADIUS: usize = 12;
/// Text keeps this far inside a field or pad.
const INSET: usize = 14;

/// Editable fields, in BottomLeft order, with their units.
pub const FIELDS: [&[u8]; 2] = [b"FTP WATTS", b"LTHR BPM"];
/// Each press or tap moves a field by this much.
pub const STEP: u16 = 5;

/// Lines the runtime prepares for this page.
pub const TITLE: usize = 0;
/// Field values, then the zone each field anchors, in `FIELDS` order.
pub const VALUES: usize = 1;
pub const ANCHORS: usize = 3;
/// Whether the profile is saved, or why not.
pub const STATUS: usize = 5;

const LEFT: usize = 12;
const RIGHT: usize = 228;
const BOX_TOP: usize = 50;
const BOX_PITCH: usize = 78;
const BOX_HEIGHT: usize = 72;
/// Minus and plus pads, shown while a field is edited.
const PAD_TOP: usize = 208;
const PAD_HEIGHT: usize = 58;
const PAD_SPLIT: usize = 120;
const PAD_GAP: usize = 4;
const STATUS_TOP: usize = 272;

/// What a touch landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Field(usize),
    Lower,
    Raise,
}

/// The target under a touch at panel point (`x`, `y`). Pads only count while
/// a field is being edited.
pub fn target(x: usize, y: usize, editing: bool) -> Option<Target> {
    if !(LEFT..RIGHT).contains(&x) {
        return None;
    }
    if let Some(field) = (0..FIELDS.len()).find(|i| {
        let top = BOX_TOP + i * BOX_PITCH;
        (top..top + BOX_HEIGHT).contains(&y)
    }) {
        return Some(Target::Field(field));
    }
    if editing && (PAD_TOP..PAD_TOP + PAD_HEIGHT).contains(&y) {
        return Some(if x < PAD_SPLIT {
            Target::Lower
        } else {
            Target::Raise
        });
    }
    None
}

/// The fields, the pads while editing, the title and the save status.
pub(super) fn prepare(view: &View) -> Canvas {
    let lines = &view.lines;
    let mut canvas = Canvas::default();
    canvas.caption(lines[TITLE].as_bytes(), RIGHT - LEFT, LEFT, 40, Align::Left);
    for (i, label) in FIELDS.iter().enumerate() {
        let top = BOX_TOP + i * BOX_PITCH;
        let rect = Rect {
            left: LEFT,
            top,
            right: RIGHT,
            bottom: top + BOX_HEIGHT,
        };
        // Every field can be tapped, so each sits on a card; the selected one
        // is raised, and accent while it is edited.
        let selected = view.cursor == i;
        let editing = selected && view.editing;
        let color = match (selected, editing) {
            (_, true) => ACCENT,
            (true, false) => RAISED,
            _ => CARD,
        };
        canvas.shape(Shape::Panel {
            rect,
            radius: RADIUS,
            color,
        });
        // The field's name, its value, then the zone it anchors.
        let quiet = if editing { WHITE } else { SECONDARY };
        let (left, width) = (LEFT + INSET, RIGHT - LEFT - 2 * INSET);
        canvas.fit(
            &[&CAPTION],
            label,
            width,
            left,
            top + 22,
            Align::Left,
            quiet,
        );
        let value = lines[VALUES + i].as_bytes();
        canvas.fit(
            &[&VALUE, &BODY],
            value,
            width,
            left,
            top + 52,
            Align::Left,
            WHITE,
        );
        let anchor = lines[ANCHORS + i].as_bytes();
        canvas.fit(
            &[&CAPTION],
            anchor,
            width,
            left,
            top + 66,
            Align::Left,
            quiet,
        );
    }
    if view.editing {
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
            let middle = (left + right) / 2;
            canvas.fit(
                &[&LARGE],
                label,
                right - left,
                middle,
                base,
                Align::Center,
                WHITE,
            );
        }
    }
    let status = lines[STATUS].as_bytes();
    let fine = matches!(status, b"SAVED" | b"SAVING");
    let color = if fine { SECONDARY } else { AMBER };
    canvas.fit(
        &[&CAPTION],
        status,
        RIGHT - LEFT,
        LEFT,
        STATUS_TOP + 12,
        Align::Left,
        color,
    );
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::workout::Page;

    fn view(cursor: usize, editing: bool) -> View {
        let mut view = View::new(Page::Settings, cursor, None);
        view.editing = editing;
        let _ = view.lines[TITLE].push_str("TRAINING PROFILE");
        let _ = view.lines[VALUES].push_str("215");
        let _ = view.lines[VALUES + 1].push_str("185");
        let _ = view.lines[ANCHORS].push_str("Z4 195-226 W");
        let _ = view.lines[ANCHORS + 1].push_str("Z5 FROM 170 BPM");
        let _ = view.lines[STATUS].push_str("SAVED");
        view.prepare();
        view
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
    fn raises_the_selected_field_and_fills_it_while_editing() {
        let moving = view(1, false);
        let colors: std::vec::Vec<_> = panels(&moving).iter().map(|(_, c)| *c).collect();
        assert_eq!(colors, [CARD, RAISED], "no pads");
        assert_eq!(moving.pixel(LEFT, BOX_TOP), 0, "rounded corner");
        assert_eq!(moving.canvas.find("215").map(|run| run.color), Some(WHITE));
        assert!(moving.canvas.find("215").unwrap().line.face().cap == VALUE.cap);
        // No permanent button labels.
        assert!((0..240).all(|x| (300..320).all(|y| moving.pixel(x, y) == 0)));

        let editing = view(1, true);
        let colors: std::vec::Vec<_> = panels(&editing).iter().map(|(_, c)| *c).collect();
        assert_eq!(colors, [CARD, ACCENT, RAISED, RAISED]);
        assert_eq!(editing.pixel(20, BOX_TOP + BOX_PITCH + 10), ACCENT);
        assert!(editing.canvas.shows("-5") && editing.canvas.shows("+5"));
        assert_eq!(editing.pixel(LEFT + 2, PAD_TOP + 20), RAISED);
    }

    /// Long values, anchors and statuses still lay out whole on every field.
    #[test]
    fn extreme_settings_never_overlap_or_clip() {
        let screen = Rect {
            left: 0,
            top: 24,
            right: 240,
            bottom: 320,
        };
        for cursor in 0..FIELDS.len() {
            for editing in [false, true] {
                for status in ["SAVED", "SAVE FAILED, RETRYING", "NOT SAVED: DATA KEPT"] {
                    let mut view = View::new(Page::Settings, cursor, None);
                    view.editing = editing;
                    let _ = view.lines[TITLE].push_str("TRAINING PROFILE");
                    let _ = view.lines[VALUES].push_str("1000");
                    let _ = view.lines[VALUES + 1].push_str("220");
                    let _ = view.lines[ANCHORS].push_str("Z4 905-1050 W");
                    let _ = view.lines[ANCHORS + 1].push_str("Z5 FROM 220 BPM");
                    let _ = view.lines[STATUS].push_str(status);
                    view.prepare();
                    view.canvas.assert_laid_out(
                        screen,
                        &["1000", "220", "Z4 905-1050 W", "Z5 FROM 220 BPM", status],
                        &std::format!("{cursor} {editing} {status}"),
                    );
                }
            }
        }
    }

    #[test]
    fn touches_find_fields_and_pads_only_while_editing() {
        assert_eq!(target(100, BOX_TOP + 10, false), Some(Target::Field(0)));
        assert_eq!(
            target(200, BOX_TOP + BOX_PITCH + 70, true),
            Some(Target::Field(1))
        );
        assert_eq!(target(100, BOX_TOP + BOX_HEIGHT + 2, false), None);
        assert_eq!(target(30, PAD_TOP + 10, false), None);
        assert_eq!(target(30, PAD_TOP + 10, true), Some(Target::Lower));
        assert_eq!(target(200, PAD_TOP + 10, true), Some(Target::Raise));
        assert_eq!(target(5, PAD_TOP + 10, true), None);
    }
}
