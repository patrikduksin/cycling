use crate::compose::data::Data;
use crate::compose::kit::px;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Align;
use crate::screens::scaled::Scaled;
use crate::screens::sensors::widest;
use crate::screens::theme::LAYOUT;
use firmware_shell::rendering::font::SPILL;

pub struct SensorHint;

const HINTS: [&[u8]; 3] = [
    b"TAP TO ADD ONE OR MORE",
    b"BACK WHEN DONE",
    b"BACK, THEN FORGET ONE",
];

impl Tile for SensorHint {
    type Arg = ();
    const NAME: &'static str = "sensor hint";

    fn forms((): (), at: &Scaled) -> Forms {
        let space = &LAYOUT.space;
        let tag = at.face(LAYOUT.face.tag);
        forms(&[Form {
            width: (2 * px(at, space.gutter) + widest::of(tag, HINTS)) as u16,
            height: (tag.fit_height() + 2 * px(at, space.pad)) as u16,
            shapes: 0,
            runs: 1,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(menu) = data.sensors() else {
            return;
        };
        let rect = placed.rect.rect();
        let tag = at.face(LAYOUT.face.tag);
        let base = rect.top + (rect.height() + usize::from(tag.cap)) / 2;
        let centre = rect.left + rect.width().saturating_sub(SPILL) / 2;
        out.fit(
            &[tag],
            menu.hint().as_bytes(),
            rect.width(),
            centre,
            base,
            Align::Center,
            data.theme().sub,
        );
    }
}
