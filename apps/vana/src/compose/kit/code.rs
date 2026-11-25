use super::px;
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::WHITE;
use crate::screens::phone::{MODULE_UNITS, side};
use crate::screens::scaled::Scaled;
use crate::screens::theme::LAYOUT;

pub struct CodeTile;

impl Tile for CodeTile {
    type Arg = ();
    const NAME: &'static str = "code";

    fn forms((): (), at: &Scaled) -> Forms {
        let side = side(at.unit(MODULE_UNITS)) as u16;
        forms(&[Form {
            width: side,
            height: side,
            shapes: 1,
            runs: 0,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, _: &dyn Data, out: &mut Canvas) {
        out.shape(Shape::Panel {
            rect: placed.rect.rect(),
            radius: px(at, LAYOUT.radius.card),
            color: WHITE,
        });
    }
}
