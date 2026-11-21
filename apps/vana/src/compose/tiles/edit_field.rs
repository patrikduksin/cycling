//! The number being edited, on an accent field: what it is, its value and
//! a note under it, such as the zone it anchors.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{ACCENT, Align, WHITE};
use crate::screens::scaled::Scaled;
use firmware_shell::rendering::font::FaceRole;

pub struct EditField;

const HEIGHT_UNITS: usize = 92;
const INSET_UNITS: usize = 16;
const RADIUS_UNITS: usize = 12;
/// From the field's top to each line's capitals' end.
const LABEL_UNITS: usize = 24;
const VALUE_UNITS: usize = 60;
const NOTE_UNITS: usize = 80;

impl Tile for EditField {
    type Arg = ();
    const NAME: &'static str = "edited";

    fn forms((): (), at: &Scaled) -> Forms {
        let value = at.face(FaceRole::Value).fit_width(b"1000");
        forms(&[Form {
            width: (value + at.unit(2 * INSET_UNITS)) as u16,
            height: at.unit(HEIGHT_UNITS) as u16,
            shapes: 1,
            runs: 3,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(edited) = data.edited() else {
            return;
        };
        let rect = placed.rect.rect();
        out.shape(Shape::Panel {
            rect,
            radius: at.unit(RADIUS_UNITS),
            color: ACCENT,
        });
        let face = |role| at.face(role);
        let inset = at.unit(INSET_UNITS);
        let (left, width) = (rect.left + inset, rect.width() - 2 * inset);
        let line = |out: &mut Canvas, faces: &[_], text: &str, units| {
            let base = rect.top + at.unit(units);
            out.fit(
                faces,
                text.as_bytes(),
                width,
                left,
                base,
                Align::Left,
                WHITE,
            );
        };
        let caption = [face(FaceRole::Caption)];
        line(out, &caption, edited.label, LABEL_UNITS);
        let value = [face(FaceRole::Value), face(FaceRole::Body)];
        line(out, &value, edited.value, VALUE_UNITS);
        line(out, &caption, edited.note, NOTE_UNITS);
    }
}
