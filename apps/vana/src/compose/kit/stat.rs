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
use firmware_shell::rendering::font::Face;

pub struct StatTile;

#[derive(Debug)]
pub struct Spec {
    pub name: &'static str,
    pub slot: usize,
}

impl Spec {
    pub const fn label(&self) -> &'static str {
        self.name
    }
}

specs! {
    /// A number of Ride done. Ids: done 64–127.
    pub enum Slot: Spec {
        First = 64 => Spec { name: "FIRST", slot: 0 },
        Second = 65 => Spec { name: "SECOND", slot: 1 },
        Third = 66 => Spec { name: "THIRD", slot: 2 },
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

    fn forms(_: Slot, at: &Scaled) -> Forms {
        let (digits, note) = (at.face(LAYOUT.face.heading), at.face(LAYOUT.face.note));
        let tight = super::px(at, LAYOUT.space.tight);
        let gap = super::px(at, LAYOUT.space.gap);
        forms(&[Form {
            width: (widest(digits, &Stat::WIDEST) + tight + widest(note, &Stat::UNITS)) as u16,
            height: (digits.fit_height() + gap + note.fit_height()) as u16,
            shapes: 0,
            runs: 3,
        }])
    }

    fn prepare(slot: Slot, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(done) = data.done() else {
            return;
        };
        let stat = done.picture().stats()[slot.spec().slot];
        let theme = data.theme();
        let (digits, smaller, note) = (
            at.face(LAYOUT.face.heading),
            at.face(LAYOUT.face.label),
            at.face(LAYOUT.face.note),
        );
        let rect = placed.rect.rect();
        let mut number = Text::<8>::new();
        let unit = stat.write(data.metric(stat.metric()), &mut number);
        let base = rect.top + digits.fit_height();
        let color = theme.tone(stat.tone());
        let room = rect
            .width()
            .saturating_sub(super::px(at, LAYOUT.space.tight) + note.fit_width(unit.as_bytes()));
        let run = out.fit(
            &[digits, smaller],
            number.as_bytes(),
            room,
            rect.left,
            base,
            Align::Left,
            color,
        );
        let after = run.columns().end + super::px(at, LAYOUT.space.tight);
        out.fit(
            &[note],
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
            rect.bottom,
            Align::Left,
            theme.sub,
        );
    }
}
