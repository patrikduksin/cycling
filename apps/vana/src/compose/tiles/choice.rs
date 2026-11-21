//! A setup step's card: what it does, and a note at its right such as the
//! value a number has now. ACT or a tap takes it; the focused card is
//! raised inside an accent ring.
use crate::compose::data::{Data, PageData};
use crate::compose::input::{Action, OnPage};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{ACCENT, Align, CARD, RAISED, Rect, SECONDARY, WHITE};
use crate::screens::scaled::Scaled;
use crate::screens::setup::{CARDS, STEPS, narrow, widest_note};
use crate::screens::workout::Text;
use firmware_shell::rendering::font::{FaceRole, SPILL};

pub struct Choice;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot(pub u8);

impl Arg for Slot {
    fn decode(raw: u8) -> Option<Self> {
        (usize::from(raw) < CARDS).then_some(Self(raw))
    }

    fn encode(self) -> u8 {
        self.0
    }
}

const HEIGHT_UNITS: usize = 52;
/// How the label and its note show, best first: the label one face
/// down, then the note shedding words (`setup::narrow`), as the card's
/// width needs.
const LOOKS: [(FaceRole, bool); 3] = [
    (FaceRole::Body, false),
    (FaceRole::Caption, false),
    (FaceRole::Caption, true),
];
const RADIUS_UNITS: usize = 12;
const RING_UNITS: usize = 3;
/// The label and note keep this far from the card's sides, and from each
/// other.
const INSET_UNITS: usize = 16;
const BETWEEN_UNITS: usize = 12;

/// A card in `rect` with corners of `radius`: raised inside an accent
/// ring `ring` wide when focused, else filled with `rest` if it has one.
pub(super) fn card(
    out: &mut Canvas,
    rect: Rect,
    (radius, ring): (usize, usize),
    focused: bool,
    rest: Option<u16>,
) {
    if focused {
        out.shape(Shape::Panel {
            rect,
            radius,
            color: ACCENT,
        });
        out.shape(Shape::Panel {
            rect: rect.inset(ring),
            radius: radius - ring,
            color: RAISED,
        });
    } else if let Some(color) = rest {
        out.shape(Shape::Panel {
            rect,
            radius,
            color,
        });
    }
}

fn draw(
    Slot(slot): Slot,
    placed: &Placed,
    at: &Scaled,
    data: &dyn Data,
    out: &mut Canvas,
    focused: bool,
) {
    let PageData::Setup { step, notes } = data.page() else {
        return;
    };
    let Some(&(label, _)) = step.cards().get(usize::from(slot)) else {
        return;
    };
    let rect = placed.rect.rect();
    let corners = (at.unit(RADIUS_UNITS), at.unit(RING_UNITS));
    card(out, rect, corners, focused, Some(CARD));
    let inset = at.unit(INSET_UNITS);
    let (left, right) = (rect.left + inset, rect.right - inset);
    let note = notes
        .get(usize::from(slot))
        .map_or(&b""[..], |note| note.as_bytes());
    let caption = at.face(FaceRole::Caption);
    let (role, note) = LOOKS
        .into_iter()
        .map(|(role, narrowed)| (role, if narrowed { narrow(step, note) } else { note }))
        .find(|&(role, note)| columns(at, role, label, note) <= right - left)
        .unwrap_or((FaceRole::Caption, narrow(step, note)));
    let face = at.face(role);
    let base = rect.top + (rect.height() + face.cap as usize) / 2;
    let title = out.fit(
        &[face],
        label.as_bytes(),
        right - left,
        left,
        base,
        Align::Left,
        WHITE,
    );
    if !note.is_empty() {
        let room = right.saturating_sub(title.right() + at.unit(BETWEEN_UNITS));
        out.fit(&[caption], note, room, right, base, Align::Right, SECONDARY);
    }
}

/// The columns `label` in `role` takes with `note` beside it.
fn columns(at: &Scaled, role: FaceRole, label: &str, note: &[u8]) -> usize {
    let beside = match note {
        b"" => SPILL,
        note => at.unit(BETWEEN_UNITS) + at.face(FaceRole::Caption).fit_width(note),
    };
    at.face(role).width(label.as_bytes()) + beside
}

impl Tile for Choice {
    type Arg = Slot;
    const NAME: &'static str = "choice";

    /// As wide as the widest label in the caption face with the widest
    /// note its step writes beside it, narrowed: a wider box shows more.
    fn forms(Slot(slot): Slot, at: &Scaled) -> Forms {
        let widest = STEPS
            .iter()
            .filter_map(|&step| {
                let (label, _) = step.cards().get(usize::from(slot))?;
                let note = match slot {
                    0 => widest_note(step),
                    _ => Text::new(),
                };
                Some(columns(
                    at,
                    FaceRole::Caption,
                    label,
                    narrow(step, note.as_bytes()),
                ))
            })
            .max()
            .unwrap_or_default();
        forms(&[Form {
            width: (widest + 2 * at.unit(INSET_UNITS)) as u16,
            height: at.unit(HEIGHT_UNITS) as u16,
            shapes: 2,
            runs: 2,
        }])
    }

    fn prepare(slot: Slot, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        draw(slot, placed, at, data, out, false);
    }

    fn focused(slot: Slot, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        draw(slot, placed, at, data, out, true);
    }

    fn action(Slot(slot): Slot) -> Option<Action> {
        Some(Action::Page(OnPage::Item(slot)))
    }
}
