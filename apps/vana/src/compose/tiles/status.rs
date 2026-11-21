//! The status bar's room: the ride state, the clock and the page dots,
//! which the bar's own painter draws in the box the tree gives it.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::scaled::Scaled;

pub struct StatusBar;

/// The bar's rows; the black under it belongs to the box the tree gives.
pub const BAR_UNITS: usize = 22;

impl Tile for StatusBar {
    type Arg = ();
    const NAME: &'static str = "status";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: at.panel.width as u16,
            height: at.unit(BAR_UNITS) as u16,
            shapes: 0,
            runs: 0,
        }])
    }

    fn prepare((): (), _: &Placed, _: &Scaled, _: &dyn Data, _: &mut Canvas) {}
}
