//! A rule between neighbours: the tree sizes it along its line, and it
//! fills its whole box in the look's rule colour.
use super::px;
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::scaled::Scaled;
use crate::screens::theme::LAYOUT;

pub struct Rule;

impl Tile for Rule {
    type Arg = ();
    const NAME: &'static str = "rule";

    fn forms((): (), at: &Scaled) -> Forms {
        let thick = px(at, LAYOUT.space.rule) as u16;
        forms(&[Form {
            width: thick,
            height: thick,
            shapes: 1,
            runs: 0,
        }])
    }

    fn prepare((): (), placed: &Placed, _: &Scaled, data: &dyn Data, out: &mut Canvas) {
        out.shape(Shape::Bar {
            rect: placed.rect.rect(),
            radius: 0,
            color: data.theme().rule,
        });
    }
}
