//! The heading of a list in a caption, over the number editor: which
//! section the edited number belongs to.
use crate::compose::data::{Data, ListKind};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Align;
use crate::screens::scaled::Scaled;
use firmware_shell::rendering::font::FaceRole;

pub struct Section;

impl Tile for Section {
    type Arg = ListKind;
    const NAME: &'static str = "section";

    fn forms(list: ListKind, at: &Scaled) -> Forms {
        let face = at.face(FaceRole::Caption);
        let widest = list
            .headings()
            .iter()
            .map(|name| face.fit_width(name.as_bytes()))
            .max();
        forms(&[Form {
            width: widest.unwrap_or(0) as u16,
            ..Form::text(face, b"")
        }])
    }

    fn prepare(list: ListKind, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect;
        out.caption(
            at,
            data.heading(list).as_bytes(),
            usize::from(rect.width()),
            usize::from(rect.left),
            usize::from(rect.bottom),
            Align::Left,
        );
    }
}
