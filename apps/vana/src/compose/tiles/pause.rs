//! The pause mark: two amber bars side by side, centred in the box.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{AMBER, Rect};
use crate::screens::scaled::Scaled;

pub struct Pause;

const BAR_UNITS: usize = 14;
const BETWEEN_UNITS: usize = 12;
const HEIGHT_UNITS: usize = 40;
const RADIUS_UNITS: usize = 4;

impl Tile for Pause {
    type Arg = ();
    const NAME: &'static str = "pause";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: (2 * at.unit(BAR_UNITS) + at.unit(BETWEEN_UNITS)) as u16,
            height: at.unit(HEIGHT_UNITS) as u16,
            shapes: 2,
            runs: 0,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, _: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect.rect();
        let (bar, between) = (at.unit(BAR_UNITS), at.unit(BETWEEN_UNITS));
        let left = rect.left + (rect.width() - 2 * bar - between) / 2;
        for start in [left, left + bar + between] {
            out.shape(Shape::Bar {
                rect: Rect {
                    left: start,
                    right: start + bar,
                    top: rect.top,
                    bottom: rect.top + at.unit(HEIGHT_UNITS),
                },
                radius: at.unit(RADIUS_UNITS),
                color: AMBER,
            });
        }
    }
}
