//! Settings: the training profile, one field at a time.
//!
//! Buttons keep their system roles. BottomLeft moves to the next field,
//! BottomRight edits it and TopLeft goes back Home. While a field is being
//! edited, BottomLeft lowers and BottomRight raises it by `STEP`, and TopLeft
//! finishes. Touch does the same directly: tap a field to edit it, then tap
//! the large minus and plus pads. The runtime prepares the lines; this draws
//! them and maps touches to `Target`s.
use super::rounded;
use super::workout::View;
use firmware_shell::rendering::text::text;

const CYAN: u16 = 0x07ff;
const WHITE: u16 = 0xffff;
const DIM: u16 = 0x8410;
const YELLOW: u16 = 0xffe0;
/// A soft fill that groups without drawing borders.
const RAISED: u16 = 0x2124;
const RADIUS: usize = 12;

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

pub(super) fn pixel(view: &View, x: usize, y: usize) -> u16 {
    let lines = &view.lines;
    if text(x, y, 10, 36, 1, lines[TITLE].as_bytes()) {
        return CYAN;
    }
    let inside = (LEFT..RIGHT).contains(&x);
    for (i, label) in FIELDS.iter().enumerate() {
        let top = BOX_TOP + i * BOX_PITCH;
        if !(inside && (top..top + BOX_HEIGHT).contains(&y)) {
            continue;
        }
        let selected = view.cursor == i;
        let filled = selected && view.editing;
        let (ink, label_ink) = if filled { (0, 0) } else { (WHITE, DIM) };
        if text(x, y, 22, top + 8, 2, label) {
            return label_ink;
        }
        if text(x, y, 22, top + 28, 5, lines[VALUES + i].as_bytes()) {
            return ink;
        }
        if text(x, y, 130, top + 46, 1, lines[ANCHORS + i].as_bytes()) {
            return label_ink;
        }
        // The selected field sits on a soft panel, cyan while it is edited;
        // the others float on black.
        if !selected || !rounded(x, y, (LEFT, top, RIGHT, top + BOX_HEIGHT), RADIUS) {
            return 0;
        }
        return if filled { CYAN } else { RAISED };
    }
    if view.editing && inside && (PAD_TOP..PAD_TOP + PAD_HEIGHT).contains(&y) {
        let (left, right, label): (usize, usize, &[u8]) = if x < PAD_SPLIT {
            (LEFT, PAD_SPLIT - PAD_GAP, b"-5")
        } else {
            (PAD_SPLIT + PAD_GAP, RIGHT, b"+5")
        };
        if !(left..right).contains(&x) {
            return 0;
        }
        let middle = (left + right) / 2;
        if text(x, y, middle - 24, PAD_TOP + 15, 4, label) {
            return WHITE;
        }
        let pad = rounded(x, y, (left, PAD_TOP, right, PAD_TOP + PAD_HEIGHT), RADIUS);
        return if pad { RAISED } else { 0 };
    }
    if text(x, y, LEFT, STATUS_TOP, 1, lines[STATUS].as_bytes()) {
        return if lines[STATUS].as_bytes().starts_with(b"NOT") {
            YELLOW
        } else {
            DIM
        };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::workout::Page;

    fn view(cursor: usize, editing: bool) -> View {
        let mut view = View::new(Page::Settings, cursor, None);
        view.editing = editing;
        let _ = view.lines[VALUES].push_str("215");
        let _ = view.lines[VALUES + 1].push_str("185");
        view.prepare();
        view
    }

    fn lit(view: &View, left: usize, top: usize, scale: usize, label: &[u8], color: u16) -> bool {
        (top..top + 7 * scale).all(|y| {
            (left..left + label.len() * 6 * scale)
                .all(|x| (view.pixel(x, y) == color) == text(x, y, left, top, scale, label))
        })
    }

    #[test]
    fn frames_the_selected_field_and_fills_it_while_editing() {
        let moving = view(1, false);
        assert_eq!(moving.pixel(20, BOX_TOP + 4), 0, "unselected: no box");
        assert_eq!(moving.pixel(20, BOX_TOP + BOX_PITCH + 4), RAISED);
        assert_eq!(moving.pixel(LEFT, BOX_TOP + BOX_PITCH), 0, "rounded corner");
        assert!(lit(&moving, 22, BOX_TOP + 28, 5, b"215", WHITE));
        // No pads, and no permanent button labels.
        assert_eq!(moving.pixel(60, PAD_TOP), 0);
        assert!((0..240).all(|x| (290..320).all(|y| moving.pixel(x, y) == 0)));

        let editing = view(1, true);
        assert_eq!(editing.pixel(20, BOX_TOP + BOX_PITCH + 10), CYAN);
        assert!(lit(
            &editing,
            22,
            BOX_TOP + BOX_PITCH + 8,
            2,
            b"LTHR BPM",
            0
        ));
        assert!(lit(&editing, 40, PAD_TOP + 15, 4, b"-5", WHITE));
        assert!(lit(&editing, 152, PAD_TOP + 15, 4, b"+5", WHITE));
        assert_eq!(editing.pixel(LEFT + 2, PAD_TOP + 20), RAISED);
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
