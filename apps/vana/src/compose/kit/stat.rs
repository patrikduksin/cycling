//! One of Ride done's numbers: the number with its unit after it, and what
//! it is under them, in sentence case. Which number a slot shows follows
//! the screen's picture (`done::Picture::stats`).
use super::specs;
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Align;
use crate::screens::done::Stat;
use crate::screens::scaled::Scaled;
use crate::screens::theme::LAYOUT;
use crate::screens::workout::Text;
use firmware_shell::rendering::font::{Face, FaceRole};

pub struct StatTile;

const HERO: [(FaceRole, FaceRole); 2] = [
    (LAYOUT.face.digits_mid, LAYOUT.face.label),
    (LAYOUT.face.digits_small, LAYOUT.face.note),
];
const PAIR: [(FaceRole, FaceRole); 2] = [
    (LAYOUT.face.digits_small, LAYOUT.face.note),
    (LAYOUT.face.heading, LAYOUT.face.note),
];

#[derive(Debug)]
pub struct Spec {
    pub name: &'static str,
    pub slot: usize,
    /// The number's face and its unit's, biggest first; the last is the
    /// fallback a narrow screen, as beside a radar, gets.
    pub looks: [(FaceRole, FaceRole); 2],
}

impl Spec {
    pub const fn label(&self) -> &'static str {
        self.name
    }
}

specs! {
    /// A number of Ride done. Ids: done 64–127.
    pub enum Slot: Spec {
        First = 64 => Spec { name: "FIRST", slot: 0, looks: HERO },
        Second = 65 => Spec { name: "SECOND", slot: 1, looks: PAIR },
        Third = 66 => Spec { name: "THIRD", slot: 2, looks: PAIR },
    }
}

fn widest(face: &Face, texts: &[&str]) -> usize {
    texts
        .iter()
        .map(|text| face.fit_width(text.as_bytes()))
        .max()
        .unwrap_or(0)
}

impl Tile for StatTile {
    type Arg = Slot;
    const NAME: &'static str = "stat";

    fn forms(slot: Slot, at: &Scaled) -> Forms {
        let gap = super::px(at, LAYOUT.space.gap);
        let tight = super::px(at, LAYOUT.space.tight);
        let pad = super::px(at, LAYOUT.space.pad);
        let note = at.face(LAYOUT.face.note);
        let looks = slot.spec().looks.map(|(digits, unit)| {
            let (digits, unit) = (at.face(digits), at.face(unit));
            Form {
                width: (widest(digits, &Stat::WIDEST) + tight + widest(unit, &Stat::UNITS)) as u16,
                height: (digits.fit_height() + gap + note.fit_height() + pad) as u16,
                shapes: 0,
                runs: 3,
            }
        });
        forms(&looks)
    }

    fn prepare(slot: Slot, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(done) = data.done() else {
            return;
        };
        let stat = done.picture().stats()[slot.spec().slot];
        let theme = data.theme();
        let (digits, unit_face) = slot.spec().looks[usize::from(placed.form)];
        let (digits, unit_face, note) = (
            at.face(digits),
            at.face(unit_face),
            at.face(LAYOUT.face.note),
        );
        let rect = placed.rect.rect();
        let mut number = Text::<8>::new();
        let unit = stat.write(data.metric(stat.metric()), &mut number);
        let base = rect.top + digits.fit_height();
        let color = theme.tone(stat.tone());
        let room = rect.width().saturating_sub(
            super::px(at, LAYOUT.space.tight) + unit_face.fit_width(unit.as_bytes()),
        );
        let run = out.fit(
            &[digits],
            number.as_bytes(),
            room,
            rect.left,
            base,
            Align::Left,
            color,
        );
        let after = run.columns().end + super::px(at, LAYOUT.space.tight);
        out.fit(
            &[unit_face],
            unit.as_bytes(),
            rect.right.saturating_sub(after),
            after,
            base,
            Align::Left,
            color,
        );
        out.fit(
            &[note],
            stat.caption().as_bytes(),
            rect.width(),
            rect.left,
            rect.bottom - super::px(at, LAYOUT.space.pad),
            Align::Left,
            theme.sub,
        );
    }
}
