//! The whole workout as bars, one per played step, which the workout
//! page's chart painter fills row by row in this tile's box.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::scaled::Scaled;

pub struct WorkoutChart;

/// The fewest rows and columns the bars need.
const MIN_ROWS_UNITS: usize = 20;
const MIN_COLUMNS_UNITS: usize = 40;

impl Tile for WorkoutChart {
    type Arg = ();
    const NAME: &'static str = "workout chart";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: at.unit(MIN_COLUMNS_UNITS) as u16,
            height: at.unit(MIN_ROWS_UNITS) as u16,
            shapes: 0,
            runs: 0,
        }])
    }

    fn heavy() -> bool {
        true
    }

    fn prepare((): (), _: &Placed, _: &Scaled, _: &dyn Data, _: &mut Canvas) {}
}
