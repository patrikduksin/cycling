//! Whether Settings saved what the rider changed, at the screen's foot:
//! quiet while it saved, amber while it could not. A toast over the foot
//! hides it.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{AMBER, Align, SECONDARY};
use crate::screens::scaled::Scaled;
use firmware_shell::rendering::font::FaceRole;

pub struct SaveStatus;

impl Tile for SaveStatus {
    type Arg = ();
    const NAME: &'static str = "saving";

    fn forms((): (), at: &Scaled) -> Forms {
        let face = at.face(FaceRole::Caption);
        forms(&[Form::text(face, b"NOT SAVED: DATA KEPT")])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some((status, fine)) = data.saving() else {
            return;
        };
        let rect = placed.rect;
        out.fit(
            &[at.face(FaceRole::Caption)],
            status.as_bytes(),
            usize::from(rect.width()),
            usize::from(rect.left),
            usize::from(rect.bottom),
            Align::Left,
            if fine { SECONDARY } else { AMBER },
        );
    }
}
