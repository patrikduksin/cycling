//! A line of a list item, drawn on its card for the item the list hands
//! it: the item's name, with a dot when it is the one chosen, or the line
//! under it.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape, on_baseline};
use crate::screens::chrome::{Align, GREEN, Line as ChromeLine, MUTED, WHITE};
use crate::screens::scaled::Scaled;
use firmware_shell::rendering::font::{FaceRole, SPILL};

pub struct EntryLine;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Title,
    Detail,
}

impl Arg for Line {
    fn decode(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Title),
            1 => Some(Self::Detail),
            _ => None,
        }
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        match self {
            Self::Title => "TITLE",
            Self::Detail => "DETAIL",
        }
    }
}

/// Columns kept clear inside the card at either side.
const INSET_UNITS: usize = 16;
/// The chosen dot's radius, and the columns it keeps at the right.
const DOT_UNITS: usize = 4;
const DOT_ROOM_UNITS: usize = 16;
const NOTE_GAP_UNITS: usize = 12;

impl Tile for EntryLine {
    type Arg = Line;
    const NAME: &'static str = "entry";

    fn forms(line: Line, at: &Scaled) -> Forms {
        let role = match line {
            Line::Title => FaceRole::Body,
            Line::Detail => FaceRole::Caption,
        };
        let room = at.unit(2 * INSET_UNITS + DOT_ROOM_UNITS) as u16;
        let (shapes, runs) = match line {
            Line::Title => (1, 2),
            Line::Detail => (0, 1),
        };
        forms(&[Form {
            width: room,
            height: at.face(role).fit_height() as u16,
            shapes,
            runs,
        }])
    }

    fn prepare(line: Line, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(row) = data.row() else {
            return;
        };
        let Some(entry) = data.entry(row.list, row.index) else {
            return;
        };
        let rect = placed.rect;
        let (inset, base) = (at.unit(INSET_UNITS), usize::from(rect.bottom));
        let left = usize::from(rect.left) + inset;
        let right = usize::from(rect.right) - inset;
        match line {
            Line::Title => {
                let mut text_right = right;
                if entry.chosen {
                    text_right -= at.unit(DOT_ROOM_UNITS);
                    let middle = base - at.face(FaceRole::Body).fit_height() / 2;
                    out.shape(Shape::Disc {
                        cx: 4 * (right - at.unit(DOT_UNITS)) as i32,
                        cy: 4 * middle as i32,
                        radius: 4 * at.unit(DOT_UNITS) as i32,
                        color: GREEN,
                    });
                }
                let caption = at.face(FaceRole::Caption);
                let color = if row.focused { WHITE } else { MUTED };
                let faces = [at.face(FaceRole::Body), caption];
                let title = entry.title.as_bytes();
                let gap = at.unit(NOTE_GAP_UNITS);
                // A note that comes first keeps its room while the title,
                // in the caption face at worst, fits beside it.
                if entry.note_first {
                    let note = ChromeLine::<24>::fit(
                        &[caption],
                        entry.note.as_bytes(),
                        (text_right - left) / 2,
                    );
                    let beside = (text_right - note.width()).saturating_sub(left + gap);
                    let room = if entry.note.is_empty() || caption.width(title) + SPILL > beside {
                        text_right - left
                    } else {
                        out.run(on_baseline(
                            note,
                            text_right,
                            base,
                            Align::Right,
                            entry.note_color,
                        ));
                        beside
                    };
                    out.fit(&faces, title, room, left, base, Align::Left, color);
                    return;
                }
                let title = out.fit(
                    &faces,
                    title,
                    text_right - left,
                    left,
                    base,
                    Align::Left,
                    color,
                );
                // Otherwise a note too wide beside the title gives way to
                // its part after a comma, then to none.
                let room = text_right.saturating_sub(title.right() + gap);
                let whole = entry.note.as_bytes();
                let last = whole
                    .windows(2)
                    .position(|pair| pair == b", ")
                    .map_or(whole, |comma| &whole[comma + 2..]);
                let fits = |note: &[u8]| {
                    let line = ChromeLine::<24>::fit(&[caption], note, room);
                    !note.is_empty() && line.text().len() == note.len()
                };
                if let Some(note) = [whole, last].into_iter().find(|note| fits(note)) {
                    let color = entry.note_color;
                    out.fit(
                        &[caption],
                        note,
                        room,
                        text_right,
                        base,
                        Align::Right,
                        color,
                    );
                }
            }
            Line::Detail => {
                out.caption(
                    at,
                    entry.detail.as_bytes(),
                    right - left,
                    left,
                    base,
                    Align::Left,
                );
            }
        }
    }
}
