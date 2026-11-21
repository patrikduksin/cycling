//! A list's heading: its name, or the section it shows, and where the
//! focus is among its items.
use crate::compose::data::{Data, ListKind};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, WHITE};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole;

pub struct Heading;

const FACES: [FaceRole; 2] = [FaceRole::Title, FaceRole::Body];
/// The columns the place ("12/40") keeps at the right.
const PLACE_UNITS: usize = 60;

impl Arg for ListKind {
    fn decode(raw: u8) -> Option<Self> {
        Self::from_id(raw)
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        self.name()
    }
}

impl Tile for Heading {
    type Arg = ListKind;
    const NAME: &'static str = "heading";

    fn forms(list: ListKind, at: &Scaled) -> Forms {
        let place = if list.placed() {
            at.unit(PLACE_UNITS)
        } else {
            0
        };
        FACES
            .iter()
            .map(|&role| {
                let face = at.face(role);
                let widest = list
                    .headings()
                    .iter()
                    .map(|name| face.fit_width(name.as_bytes()));
                Form {
                    width: (widest.max().unwrap_or(0) + place) as u16,
                    runs: 1 + u8::from(list.placed()),
                    ..Form::text(face, b"")
                }
            })
            .collect()
    }

    fn prepare(list: ListKind, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect;
        let place = if list.placed() {
            at.unit(PLACE_UNITS)
        } else {
            0
        };
        let base = usize::from(rect.bottom);
        // The look fits the widest heading; a shorter one may take any
        // larger face its box is tall enough for.
        let tall = FACES
            .iter()
            .position(|&role| at.face(role).fit_height() <= usize::from(rect.height()));
        let faces = &FACES[tall.unwrap_or(usize::from(placed.form))..];
        let faces = [
            at.face(faces[0]),
            at.face(*faces.last().unwrap_or(&faces[0])),
        ];
        out.fit(
            &faces,
            data.heading(list).as_bytes(),
            usize::from(rect.width()) - place,
            usize::from(rect.left),
            base,
            Align::Left,
            WHITE,
        );
        if let Some(focus) = data.listed(list).focus.filter(|_| list.placed()) {
            let mut position = Text::<12>::new();
            let _ = write!(position, "{}/{}", focus + 1, data.list(list));
            out.caption(
                at,
                position.as_bytes(),
                place,
                usize::from(rect.right),
                base,
                Align::Right,
            );
        }
    }
}
