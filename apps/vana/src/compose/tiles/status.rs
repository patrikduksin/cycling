//! The status bar's room: the ride state, the clock and the page dots,
//! which the bar's own painter draws in the box the tree gives it.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::scaled::Scaled;

pub struct StatusBar;

/// The rows the bar paints, as chrome lays it out: the tile keeps exactly
/// these, so a tile right under it loses none of its rows.
pub const fn rows(at: &Scaled) -> usize {
    crate::screens::chrome::status_bar(at).bottom
}

impl Tile for StatusBar {
    type Arg = ();
    const NAME: &'static str = "status";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: at.panel.width as u16,
            height: rows(at) as u16,
            shapes: 0,
            runs: 0,
        }])
    }

    fn prepare((): (), _: &Placed, _: &Scaled, _: &dyn Data, _: &mut Canvas) {}
}
