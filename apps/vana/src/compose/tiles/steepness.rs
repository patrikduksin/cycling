//! A band in the grade's steepness colour, grey without a grade.
use crate::compose::data::{Data, Metric};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::ride::climb::Steepness as Band;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::TERTIARY;
use crate::screens::ride::climb::color;
use crate::screens::scaled::Scaled;

pub struct Steepness;

const BAND_UNITS: usize = 5;
const RADIUS_UNITS: usize = 2;

impl Tile for Steepness {
    type Arg = ();
    const NAME: &'static str = "steepness";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: at.unit(4 * RADIUS_UNITS) as u16,
            height: at.unit(BAND_UNITS) as u16,
            shapes: 1,
            runs: 0,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let steepness = data.metric(Metric::Grade).map(Band::of);
        out.shape(Shape::Bar {
            rect: placed.rect.rect(),
            radius: at.unit(RADIUS_UNITS),
            color: steepness.map_or(TERTIARY, color),
        });
    }
}
