use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::scaled::Scaled;

pub struct Space;

impl Tile for Space {
    type Arg = ();
    const NAME: &'static str = "space";

    fn forms((): (), _: &Scaled) -> Forms {
        forms(&[Form {
            width: 0,
            height: 0,
            shapes: 0,
            runs: 0,
        }])
    }

    fn prepare((): (), _: &Placed, _: &Scaled, _: &dyn Data, _: &mut Canvas) {}
}
